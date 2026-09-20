//! Maud-based HTML templates. Server-rendered, no build step, no SPA.
//!
//! Editing is a plain `<textarea>` over raw markdown — no WYSIWYG, per
//! DESIGN.md §10/§14.
//!
//! ## Why there is no JavaScript
//!
//! Not asceticism: it is what lets the Content-Security-Policy say
//! `script-src 'none'`, which is the strongest backstop available against
//! markup ever escaping into a page. Everything here — theming, dark mode,
//! layout, focus behaviour — is CSS. The first script that lands costs that
//! guarantee, so it should have to earn it.
//!
//! ## What the design is trying to say
//!
//! This wiki's premise is that humans, agents and system processes write to
//! one substrate and every write is attributed (§3.1). A design that renders
//! all of them as anonymous grey text would contradict the product. So actor
//! identity is a first-class visual element: every actor appears with a typed
//! badge, and pages carry *who touched this, and how long ago* at the point
//! where you read them.

use comrak::{markdown_to_html, ComrakOptions};
use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::auth::Auth;
use crate::db::queries::Actor;
use crate::git::CommitInfo;
use crate::pages::{Page, PageMeta, SearchHit};

/// Render an FTS5 snippet, turning match sentinels into `<mark>` and escaping
/// everything else.
///
/// `pages::search` asks SQLite to delimit matches with control characters
/// rather than literal `<mark>` tags, because the snippet body is *raw page
/// markdown*. Emitting that unescaped — as this did — let any actor who can
/// write a page store HTML that executes in every reader's browser the moment
/// the page turned up in a search. Agents write pages automatically here, so
/// the path was reachable without a human ever choosing to run anything.
///
/// Here each fragment is interpolated as ordinary text, which maud escapes,
/// and only the sentinels become markup. A stray sentinel in page content can
/// at worst produce a spurious highlight.
fn render_snippet(raw: &str) -> Markup {
    html! {
        @for (i, part) in raw.split(crate::pages::SNIPPET_OPEN).enumerate() {
            @if i == 0 {
                (part)
            } @else {
                @match part.split_once(crate::pages::SNIPPET_CLOSE) {
                    Some((hit, rest)) => { mark { (hit) } (rest) }
                    None => { mark { (part) } }
                }
            }
        }
    }
}

/// The page body with its leading `# Title` heading removed, when that
/// heading is exactly what gave the page its title.
///
/// `derive_title` reads the first `# ` line, and the chrome renders that as
/// the page heading. Rendering the markdown unchanged then prints the page's
/// own name twice, immediately below itself. Anything else — a different
/// first heading, no heading at all — is left alone, because then the
/// markdown is saying something the chrome is not.
fn body_after_title<'a>(content: &'a str, title: &str) -> &'a str {
    let rest = content.trim_start();
    let (first, tail) = match rest.split_once('\n') {
        Some((f, t)) => (f, t),
        None => (rest, ""),
    };
    match first.trim().strip_prefix("# ") {
        Some(h) if h.trim() == title => tail.trim_start_matches(['\n', '\r']),
        _ => content,
    }
}

pub fn render_markdown(src: &str) -> Markup {
    let mut opts = ComrakOptions::default();
    opts.extension.strikethrough = true;
    opts.extension.table = true;
    opts.extension.autolink = true;
    opts.extension.tasklist = true;
    opts.extension.footnotes = true;
    // Raw HTML in page content is never passed through. With agents writing
    // pages unattended, "trusted author" is not a category that exists here.
    opts.render.unsafe_ = false;
    PreEscaped(markdown_to_html(src, &opts))
}

// ---------------------------------------------------------------------------
// Shared pieces
// ---------------------------------------------------------------------------

/// An actor, rendered with its type. The dot colour is the type; the label
/// spells it out, because colour alone is not an accessible distinction.
fn actor_chip(handle: &str, actor_type: Option<&str>) -> Markup {
    let ty = actor_type.unwrap_or("unknown");
    html! {
        span class={ "actor actor--" (ty) } {
            span."actor__dot" aria-hidden="true" {}
            span."actor__handle" { (handle) }
            span."actor__type" { (ty) }
        }
    }
}

/// The memory layer a page belongs to (§5). Unobtrusive but always present —
/// a reader should never have to guess whether they are looking at settled
/// knowledge or a log entry.
fn layer_chip(layer: &str) -> Markup {
    html! {
        span class={ "chip chip--" (layer) } title={ "memory layer: " (layer) } { (layer) }
    }
}

/// "edited by <actor> · 2h ago", or nothing when the page has no recorded
/// editor yet.
fn attribution(meta: &PageMeta) -> Markup {
    html! {
        @if let Some(handle) = meta.last_edit_handle.as_deref() {
            span."attribution" {
                span."attribution__label" { "edited by" }
                (actor_chip(handle, meta.last_edit_type.as_deref()))
                @if let Some(at) = meta.last_edit_at {
                    span."dot-sep" aria-hidden="true" { "·" }
                    time datetime=(iso_time(at)) { (rel_time(at)) }
                }
            }
        }
    }
}

fn layout(title: &str, auth: Option<&Auth>, body: Markup) -> Markup {
    let title_full = format!("{title} · wikikiki");
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                title { (title_full) }
                style { (PreEscaped(STYLES)) }
            }
            body {
                a."skip-link" href="#main" { "Skip to content" }
                header."topbar" {
                    a."brand" href="/" {
                        span."brand__mark" aria-hidden="true" {}
                        span."brand__name" { "wikikiki" }
                    }
                    nav."nav" aria-label="Main" {
                        a."nav__link" href="/" { "Pages" }
                        a."nav__link" href="/search" { "Search" }
                        a."nav__link" href="/actors" { "Actors" }
                        @if let Some(a) = auth {
                            a."nav__me" href="/me" {
                                (actor_chip(&a.actor.handle, Some(&a.actor.actor_type)))
                            }
                            form."inline-form" method="post" action="/logout" {
                                button."btn btn--ghost btn--sm" type="submit" { "Log out" }
                            }
                        } @else {
                            a."btn btn--sm" href="/login" { "Log in" }
                        }
                    }
                }
                main #main { (body) }
                footer."footer" {
                    span { "wikikiki — the wiki is the memory." }
                }
            }
        }
    }
}

fn wiki_url(path: &str) -> String {
    format!("/wiki/{path}")
}

fn wiki_edit_url(path: &str) -> String {
    format!("/wiki/{path}?edit=1")
}

fn wiki_source_url(path: &str) -> String {
    format!("/wiki/{path}?source=1")
}

fn wiki_history_url(path: &str) -> String {
    format!("/wiki-history/{path}")
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

pub fn home(auth: Option<&Auth>, recent: &[PageMeta]) -> Markup {
    let body = html! {
        section."hero" {
            h1."hero__title" { "The wiki is the memory." }
            p."hero__lede" {
                "Shared, attributed working memory for humans, agents and system \
                 processes. Every write carries an actor; every change is recoverable."
            }
        }

        section."panel" {
            div."panel__head" {
                h2."panel__title" { "Recently touched" }
                @if !recent.is_empty() {
                    span."panel__count" { (recent.len()) " pages" }
                }
            }

            @if recent.is_empty() {
                div."empty" {
                    p."empty__title" { "Nothing here yet." }
                    p."empty__body" {
                        "The substrate is empty. "
                        @if auth.is_some() {
                            a href=(wiki_edit_url("welcome")) { "Write the first page" }
                            " — or point an agent at the REST API."
                        } @else {
                            a href="/login" { "Log in" }
                            " to write the first page."
                        }
                    }
                }
            } @else {
                ul."page-list" {
                    @for p in recent {
                        li."page-list__item" {
                            a."page-list__link" href=(wiki_url(&p.path)) {
                                span."page-list__title" { (p.title) }
                                span."page-list__path" { (p.path) }
                            }
                            div."page-list__meta" {
                                (layer_chip(&p.layer))
                                (attribution(p))
                            }
                        }
                    }
                }
            }
        }
    };
    layout("Pages", auth, body)
}

pub fn login(auth: Option<&Auth>, error: Option<&str>) -> Markup {
    let body = html! {
        div."narrow" {
            h1 { "Log in" }
            @if let Some(e) = error {
                div."alert alert--error" role="alert" { (e) }
            }
            form."form card" method="post" action="/login" {
                label."field" {
                    span."field__label" { "Handle" }
                    input."input" type="text" name="handle" autocomplete="username"
                          autocapitalize="none" spellcheck="false" required autofocus;
                }
                label."field" {
                    span."field__label" { "Password" }
                    input."input" type="password" name="password"
                          autocomplete="current-password" required;
                }
                button."btn btn--block" type="submit" { "Log in" }
            }
            p."hint" {
                "Agents authenticate with bearer tokens against the REST API. \
                 Tokens are issued by an admin with "
                code { "wikikiki issue-token" }
                "."
            }
        }
    };
    layout("Log in", auth, body)
}

pub fn view_page(auth: Option<&Auth>, page: &Page) -> Markup {
    let edit_url = wiki_edit_url(&page.meta.path);
    let history_url = wiki_history_url(&page.meta.path);
    let source_url = wiki_source_url(&page.meta.path);
    let body = html! {
        article."page" {
            header."page__head" {
                div."page__crumbs" {
                    (layer_chip(&page.meta.layer))
                    span."path" { (page.meta.path) }
                }
                h1."page__title" { (page.meta.title) }
                div."page__byline" { (attribution(&page.meta)) }
                div."page__actions" {
                    @if auth.is_some() {
                        a."btn btn--sm" href=(edit_url) { "Edit" }
                    }
                    a."btn btn--ghost btn--sm" href=(history_url) { "History" }
                    a."btn btn--ghost btn--sm" href=(source_url) { "Source" }
                }
            }
            div."prose" {
                (render_markdown(body_after_title(&page.content, &page.meta.title)))
            }
        }
    };
    layout(&page.meta.title, auth, body)
}

pub fn edit_page(
    auth: Option<&Auth>,
    page_path: &str,
    content: &str,
    meta: Option<&PageMeta>,
) -> Markup {
    let title = meta
        .map(|m| m.title.clone())
        .unwrap_or_else(|| page_path.to_string());
    let layer = meta.map(|m| m.layer.as_str()).unwrap_or("semantic");
    let post_url = wiki_url(page_path);
    let is_new = meta.is_none();
    let body = html! {
        form."editor" method="post" action=(post_url) {
            header."editor__head" {
                div."editor__ident" {
                    span."eyebrow" { @if is_new { "New page" } @else { "Editing" } }
                    h1."editor__title" { (title) }
                    span."path" { (page_path) }
                }
                div."editor__actions" {
                    a."btn btn--ghost btn--sm" href=(post_url) { "Cancel" }
                    button."btn btn--sm" type="submit" { "Save" }
                }
            }

            input type="hidden" name="layer" value=(layer);

            label."field" {
                span."field__label" { "Summary" }
                input."input" type="text" name="summary"
                      placeholder="what changed, and why" autocomplete="off";
            }

            label."field field--grow" {
                span."field__label" {
                    "Markdown source"
                    span."field__note" { "what you see here is what gets saved" }
                }
                textarea."textarea" name="content" rows="26" spellcheck="false" autofocus {
                    (content)
                }
            }
        }
    };
    layout("Edit", auth, body)
}

pub fn missing_page(auth: Option<&Auth>, page_path: &str) -> Markup {
    let edit_url = wiki_edit_url(page_path);
    let body = html! {
        div."narrow" {
            div."empty empty--tall" {
                p."empty__title" { "No page here yet" }
                p."empty__body" {
                    code."path" { (page_path) }
                }
                @if auth.is_some() {
                    a."btn" href=(edit_url) { "Create this page" }
                } @else {
                    p."hint" { a href="/login" { "Log in" } " to create it." }
                }
            }
        }
    };
    layout("Not found", auth, body)
}

pub fn history(
    auth: Option<&Auth>,
    page_path: &str,
    meta: Option<&PageMeta>,
    commits: &[CommitInfo],
) -> Markup {
    let title = meta
        .map(|m| m.title.clone())
        .unwrap_or_else(|| page_path.to_string());
    let back_url = wiki_url(page_path);
    let body = html! {
        header."page__head" {
            div."page__crumbs" {
                a."back" href=(back_url) { "← " (title) }
            }
            h1."page__title" { "History" }
            span."path" { (page_path) }
        }

        @if commits.is_empty() {
            div."empty" { p."empty__title" { "No commits recorded for this page." } }
        } @else {
            ol."timeline" {
                @for c in commits {
                    li."timeline__item" {
                        div."timeline__marker" aria-hidden="true" {}
                        div."timeline__body" {
                            div."timeline__msg" { (c.message) }
                            div."timeline__meta" {
                                (commit_author_chip(&c.author_name))
                                span."dot-sep" aria-hidden="true" { "·" }
                                time datetime=(iso_time(c.time)) { (rel_time(c.time)) }
                                span."dot-sep" aria-hidden="true" { "·" }
                                code."oid" { (short_oid(&c.oid)) }
                            }
                        }
                    }
                }
            }
        }
    };
    layout("History", auth, body)
}

/// Git author lines are rendered from `git.author_template`, whose default is
/// `{actor_type}:{actor_handle}`. Split it back apart so history shows the
/// same typed badge as everywhere else — and fall back gracefully for commits
/// written by another tool or an older template.
fn commit_author_chip(author_name: &str) -> Markup {
    match author_name.split_once(':') {
        Some((ty, handle)) if !ty.is_empty() && !handle.is_empty() => {
            actor_chip(handle, Some(ty))
        }
        _ => actor_chip(author_name, None),
    }
}

pub fn search(auth: Option<&Auth>, query: &str, hits: &[SearchHit]) -> Markup {
    let body = html! {
        div."search-head" {
            h1 { "Search" }
            form."search-form" method="get" action="/search" role="search" {
                input."input input--lg" type="text" name="q" value=(query)
                      placeholder="search every layer…" autocomplete="off"
                      spellcheck="false" autofocus;
                button."btn" type="submit" { "Search" }
            }
        }

        @if query.trim().is_empty() {
            p."hint" { "Each word is matched as a prefix, across titles and page bodies." }
        } @else if hits.is_empty() {
            div."empty" {
                p."empty__title" { "No matches" }
                p."empty__body" { "Nothing indexed matches " code { (query) } "." }
            }
        } @else {
            p."hint" { (hits.len()) @if hits.len() == 1 { " result" } @else { " results" } }
            ul."results" {
                @for h in hits {
                    li."results__item" {
                        a."results__link" href=(wiki_url(&h.path)) { (h.title) }
                        span."path" { (h.path) }
                        p."results__snippet" { (render_snippet(&h.snippet)) }
                    }
                }
            }
        }
    };
    layout("Search", auth, body)
}

pub fn me(auth: &Auth) -> Markup {
    let body = html! {
        div."narrow" {
            h1 { "You are " (auth.actor.handle) }
            div."card card--pad" {
                dl."deflist" {
                    dt { "Identity" }
                    dd { (actor_chip(&auth.actor.handle, Some(&auth.actor.actor_type))) }
                    dt { "Name" }
                    dd { (auth.actor.name) }
                    dt { "Capability" }
                    dd {
                        @if auth.actor.is_admin {
                            span."chip chip--admin" { "admin" }
                        } @else {
                            span."muted" { "standard" }
                        }
                    }
                    dt { "Surface" }
                    dd { code { (auth.surface.as_str()) } }
                }
            }
            p."hint" { "Token management and profile pages land in a later stage." }
        }
    };
    layout("Me", Some(auth), body)
}

pub fn actors_list(auth: Option<&Auth>, actors: &[Actor]) -> Markup {
    let body = html! {
        header."page__head" {
            h1."page__title" { "Actors" }
            p."hint" {
                "Everything that reads or writes is an actor. Type is fixed at \
                 issuance and cannot be changed through any surface."
            }
        }

        @if actors.is_empty() {
            div."empty" {
                p."empty__title" { "No actors yet" }
                p."empty__body" { "Run " code { "wikikiki bootstrap" } " to create the first admin." }
            }
        } @else {
            ul."actor-list" {
                @for a in actors {
                    li."actor-list__item" {
                        (actor_chip(&a.handle, Some(&a.actor_type)))
                        span."actor-list__name" { (a.name) }
                        span."actor-list__status" {
                            @if !a.is_active {
                                span."chip chip--revoked" { "revoked" }
                            } @else if a.is_admin {
                                span."chip chip--admin" { "admin" }
                            } @else {
                                span."muted" { "active" }
                            }
                        }
                    }
                }
            }
        }
    };
    layout("Actors", auth, body)
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn short_oid(oid: &str) -> String {
    oid.chars().take(8).collect()
}

fn fmt_time(unix_ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix_ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| unix_ts.to_string())
}

/// Machine-readable stamp for `<time datetime>`, so the exact instant is
/// available on hover and to assistive tech even though the visible text is
/// relative.
fn iso_time(unix_ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix_ts, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| unix_ts.to_string())
}

/// Coarse relative time. Anything older than about a month reverts to an
/// absolute date, where "6 weeks ago" stops being the more useful answer.
fn rel_time(unix_ts: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let delta = now - unix_ts;
    match delta {
        // Meaningfully in the future. Writers on different machines have
        // different clocks, and "in 3 hours" for something already recorded
        // is a lie either way — show the stamp and let the reader judge.
        // This arm has to come first: a guard chain is ordered, so testing
        // `d < 45` before `d < 0` would swallow every negative delta.
        d if d < -60 => fmt_time(unix_ts),
        d if d < 45 => "just now".to_string(),
        d if d < 3_600 => format!("{}m ago", (d + 30) / 60),
        d if d < 86_400 => format!("{}h ago", d / 3_600),
        d if d < 2_592_000 => format!("{}d ago", d / 86_400),
        _ => fmt_time(unix_ts),
    }
}

const STYLES: &str = r#"
/* --------------------------------------------------------------------------
   Tokens. Light values here; dark overrides below. Everything downstream
   refers to these, so retheming is a one-block edit.
   -------------------------------------------------------------------------- */
:root {
  color-scheme: light dark;

  --bg:            #fbfbfc;
  --surface:       #ffffff;
  --surface-sunk:  #f4f5f7;
  --fg:            #14161a;
  --fg-muted:      #5c6370;
  --fg-subtle:     #878d99;
  --border:        #e6e8ec;
  --border-strong: #d2d6dd;

  --accent:        #4f46e5;
  --accent-hover:  #4338ca;
  --accent-fg:     #ffffff;
  --accent-soft:   #eef2ff;
  --focus:         #6366f1;

  --danger-bg:     #fef2f2;
  --danger-fg:     #b42318;
  --danger-border: #fecdca;

  --mark-bg:       #fef3c7;
  --mark-fg:       #7c4a03;

  /* Actor types. Hue carries meaning; the written label carries it too. */
  --actor-human:    #d97706;
  --actor-agent:    #7c3aed;
  --actor-system:   #64748b;
  --actor-external: #0d9488;
  --actor-unknown:  #9ca3af;

  /* Memory layers (DESIGN.md §5): live → what happened → settled. */
  --layer-working:  #dc2626;
  --layer-episodic: #2563eb;
  --layer-semantic: #059669;

  --radius:    10px;
  --radius-sm: 7px;
  --radius-pill: 999px;

  --shadow-sm: 0 1px 2px rgb(16 18 26 / .05);
  --shadow:    0 1px 3px rgb(16 18 26 / .07), 0 8px 24px -12px rgb(16 18 26 / .12);

  --font: ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto,
          "Helvetica Neue", Arial, sans-serif;
  --mono: ui-monospace, SFMono-Regular, "SF Mono", "JetBrains Mono",
          "Cascadia Code", Menlo, Consolas, monospace;

  --measure: 70ch;
  --gutter: clamp(1rem, 4vw, 2.5rem);
}

@media (prefers-color-scheme: dark) {
  :root {
    --bg:            #0c0e12;
    --surface:       #14171d;
    --surface-sunk:  #1a1e26;
    --fg:            #e8eaee;
    --fg-muted:      #9aa2b1;
    --fg-subtle:     #6f7784;
    --border:        #242932;
    --border-strong: #333946;

    --accent:        #818cf8;
    --accent-hover:  #a5b4fc;
    --accent-fg:     #12141a;
    --accent-soft:   #1e2130;
    --focus:         #a5b4fc;

    --danger-bg:     #2a1517;
    --danger-fg:     #fca5a5;
    --danger-border: #4c2225;

    --mark-bg:       #4a3410;
    --mark-fg:       #fde68a;

    --actor-human:    #fbbf24;
    --actor-agent:    #a78bfa;
    --actor-system:   #94a3b8;
    --actor-external: #2dd4bf;
    --actor-unknown:  #6b7280;

    --layer-working:  #f87171;
    --layer-episodic: #60a5fa;
    --layer-semantic: #34d399;

    --shadow-sm: 0 1px 2px rgb(0 0 0 / .4);
    --shadow:    0 1px 3px rgb(0 0 0 / .5), 0 8px 24px -12px rgb(0 0 0 / .7);
  }
}

/* --------------------------------------------------------------------------
   Base
   -------------------------------------------------------------------------- */
*, *::before, *::after { box-sizing: border-box; }

html { -webkit-text-size-adjust: 100%; }

body {
  margin: 0;
  background: var(--bg);
  color: var(--fg);
  font-family: var(--font);
  font-size: 15.5px;
  line-height: 1.6;
  -webkit-font-smoothing: antialiased;
  text-rendering: optimizeLegibility;
}

a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; text-underline-offset: 2px; }

:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
  border-radius: 3px;
}

code, kbd, samp, pre { font-family: var(--mono); font-size: .88em; }

.skip-link {
  position: absolute; left: -9999px;
  background: var(--accent); color: var(--accent-fg);
  padding: .6rem 1rem; border-radius: var(--radius-sm); z-index: 10;
}
.skip-link:focus { left: 1rem; top: 1rem; }

@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { animation: none !important; transition: none !important; }
}

/* --------------------------------------------------------------------------
   Top bar
   -------------------------------------------------------------------------- */
.topbar {
  position: sticky; top: 0; z-index: 5;
  display: flex; align-items: center; gap: 1rem;
  padding: .7rem var(--gutter);
  background: color-mix(in srgb, var(--surface) 88%, transparent);
  backdrop-filter: saturate(1.6) blur(12px);
  border-bottom: 1px solid var(--border);
}

.brand { display: inline-flex; align-items: center; gap: .55rem; color: var(--fg); }
.brand:hover { text-decoration: none; }
.brand__name { font-weight: 640; letter-spacing: -.015em; }
.brand__mark {
  width: 15px; height: 15px; border-radius: 5px;
  background: linear-gradient(135deg, var(--actor-human), var(--actor-agent) 55%, var(--actor-external));
}

.nav { display: flex; align-items: center; gap: .3rem; margin-left: auto; flex-wrap: wrap; }
.nav__link {
  color: var(--fg-muted); padding: .35rem .6rem;
  border-radius: var(--radius-sm); font-size: .92rem; font-weight: 500;
}
.nav__link:hover { color: var(--fg); background: var(--surface-sunk); text-decoration: none; }
.nav__me { margin-left: .4rem; }
.nav__me:hover { text-decoration: none; }
.inline-form { display: inline; }

/* --------------------------------------------------------------------------
   Layout
   -------------------------------------------------------------------------- */
main {
  max-width: calc(var(--measure) + 8rem);
  margin: 0 auto;
  padding: clamp(1.5rem, 5vw, 3rem) var(--gutter) 5rem;
}
.narrow { max-width: 30rem; margin: 0 auto; }

.footer {
  text-align: center; color: var(--fg-subtle);
  font-size: .82rem; padding: 2.5rem 1rem 3.5rem;
  border-top: 1px solid var(--border);
}

/* --------------------------------------------------------------------------
   Hero + panels
   -------------------------------------------------------------------------- */
.hero { margin-bottom: 2.75rem; }
.hero__title {
  font-size: clamp(1.7rem, 4.5vw, 2.35rem);
  line-height: 1.15; letter-spacing: -.028em;
  font-weight: 680; margin: 0 0 .6rem;
}
.hero__lede { color: var(--fg-muted); max-width: 52ch; margin: 0; }

.panel__head {
  display: flex; align-items: baseline; gap: .75rem;
  padding-bottom: .6rem; margin-bottom: .25rem;
  border-bottom: 1px solid var(--border);
}
.panel__title { font-size: .82rem; font-weight: 620; margin: 0;
  text-transform: uppercase; letter-spacing: .07em; color: var(--fg-muted); }
.panel__count { margin-left: auto; font-size: .8rem; color: var(--fg-subtle); }

.card {
  background: var(--surface); border: 1px solid var(--border);
  border-radius: var(--radius); box-shadow: var(--shadow-sm);
}
.card--pad { padding: 1.25rem; }

/* --------------------------------------------------------------------------
   Page list
   -------------------------------------------------------------------------- */
.page-list { list-style: none; margin: 0; padding: 0; }
.page-list__item {
  display: flex; align-items: center; gap: 1rem; flex-wrap: wrap;
  padding: .85rem .6rem; margin: 0 -.6rem;
  border-bottom: 1px solid var(--border);
  border-radius: var(--radius-sm);
  transition: background .12s ease;
}
.page-list__item:hover { background: var(--surface-sunk); }
.page-list__item:last-child { border-bottom: 0; }
.page-list__link { display: flex; flex-direction: column; gap: .1rem; min-width: 0; flex: 1; }
.page-list__link:hover { text-decoration: none; }
.page-list__title { color: var(--fg); font-weight: 560; letter-spacing: -.01em; }
.page-list__item:hover .page-list__title { color: var(--accent); }
.page-list__path {
  font-family: var(--mono); font-size: .77rem; color: var(--fg-subtle);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.page-list__meta { display: flex; align-items: center; gap: .6rem; flex-wrap: wrap; }

/* --------------------------------------------------------------------------
   Actor + layer chips
   -------------------------------------------------------------------------- */
.actor {
  display: inline-flex; align-items: center; gap: .38rem;
  font-size: .8rem; color: var(--fg-muted); white-space: nowrap;
}
.actor__dot {
  width: 7px; height: 7px; border-radius: 50%;
  background: var(--actor-unknown); flex: none;
}
.actor--human    .actor__dot { background: var(--actor-human); }
.actor--agent    .actor__dot { background: var(--actor-agent); }
.actor--system   .actor__dot { background: var(--actor-system); }
.actor--external .actor__dot { background: var(--actor-external); }
.actor__handle { color: var(--fg); font-weight: 520; }
.actor__type {
  font-size: .68rem; letter-spacing: .05em; text-transform: uppercase;
  color: var(--fg-subtle); padding: .08rem .35rem;
  border: 1px solid var(--border-strong); border-radius: var(--radius-pill);
}

.chip {
  display: inline-block; font-size: .7rem; font-weight: 560;
  letter-spacing: .04em; text-transform: uppercase;
  padding: .13rem .5rem; border-radius: var(--radius-pill);
  background: var(--surface-sunk); color: var(--fg-muted);
  border: 1px solid var(--border);
}
.chip--working  { color: var(--layer-working);
  border-color: color-mix(in srgb, var(--layer-working) 35%, transparent);
  background: color-mix(in srgb, var(--layer-working) 9%, transparent); }
.chip--episodic { color: var(--layer-episodic);
  border-color: color-mix(in srgb, var(--layer-episodic) 35%, transparent);
  background: color-mix(in srgb, var(--layer-episodic) 9%, transparent); }
.chip--semantic { color: var(--layer-semantic);
  border-color: color-mix(in srgb, var(--layer-semantic) 35%, transparent);
  background: color-mix(in srgb, var(--layer-semantic) 9%, transparent); }
.chip--admin    { color: var(--actor-human);
  border-color: color-mix(in srgb, var(--actor-human) 38%, transparent);
  background: color-mix(in srgb, var(--actor-human) 11%, transparent); }
.chip--revoked  { color: var(--danger-fg); border-color: var(--danger-border);
  background: var(--danger-bg); }

.attribution { display: inline-flex; align-items: center; gap: .4rem;
  font-size: .8rem; color: var(--fg-subtle); flex-wrap: wrap; }
.attribution__label { color: var(--fg-subtle); }
.dot-sep { color: var(--fg-subtle); }
.path { font-family: var(--mono); font-size: .78rem; color: var(--fg-subtle); }
.muted { color: var(--fg-subtle); }
.hint { color: var(--fg-muted); font-size: .88rem; }
.eyebrow { font-size: .72rem; text-transform: uppercase; letter-spacing: .08em;
  color: var(--fg-subtle); font-weight: 600; }

/* --------------------------------------------------------------------------
   Page view
   -------------------------------------------------------------------------- */
.page__head { margin-bottom: 1.75rem; }
.page__crumbs { display: flex; align-items: center; gap: .6rem;
  flex-wrap: wrap; margin-bottom: .7rem; }
.page__title {
  font-size: clamp(1.55rem, 4vw, 2.1rem); line-height: 1.2;
  letter-spacing: -.026em; font-weight: 670; margin: 0 0 .5rem;
}
.page__byline { margin-bottom: 1rem; }
.page__actions { display: flex; gap: .45rem; flex-wrap: wrap;
  padding-top: 1rem; border-top: 1px solid var(--border); }
.back { color: var(--fg-muted); font-size: .88rem; }

/* --------------------------------------------------------------------------
   Prose
   -------------------------------------------------------------------------- */
.prose { max-width: var(--measure); }
.prose > *:first-child { margin-top: 0; }
.prose h1, .prose h2, .prose h3, .prose h4 {
  letter-spacing: -.018em; line-height: 1.25; margin: 2rem 0 .7rem; font-weight: 640;
}
.prose h1 { font-size: 1.6rem; }
.prose h2 { font-size: 1.3rem; padding-bottom: .3rem; border-bottom: 1px solid var(--border); }
.prose h3 { font-size: 1.1rem; }
.prose h4 { font-size: 1rem; color: var(--fg-muted); }
.prose p, .prose ul, .prose ol { margin: 0 0 1rem; }
.prose ul, .prose ol { padding-left: 1.4rem; }
.prose li { margin: .25rem 0; }
.prose li::marker { color: var(--fg-subtle); }
.prose blockquote {
  margin: 1.25rem 0; padding: .1rem 0 .1rem 1.1rem;
  border-left: 3px solid var(--border-strong); color: var(--fg-muted);
}
.prose code {
  background: var(--surface-sunk); padding: .12em .36em;
  border-radius: 4px; border: 1px solid var(--border);
}
.prose pre {
  background: var(--surface-sunk); border: 1px solid var(--border);
  padding: .9rem 1rem; border-radius: var(--radius);
  overflow-x: auto; line-height: 1.55; margin: 1.25rem 0;
}
.prose pre code { background: none; border: 0; padding: 0; }
.prose table { border-collapse: collapse; width: 100%; margin: 1.25rem 0; font-size: .92em; }
.prose th, .prose td { border-bottom: 1px solid var(--border); padding: .5rem .7rem; text-align: left; }
.prose thead th { color: var(--fg-muted); font-weight: 600; font-size: .82em;
  text-transform: uppercase; letter-spacing: .05em; }
.prose hr { border: 0; border-top: 1px solid var(--border); margin: 2rem 0; }
.prose img { max-width: 100%; height: auto; border-radius: var(--radius-sm); }
.prose a { text-decoration: underline; text-underline-offset: 2px;
  text-decoration-color: color-mix(in srgb, var(--accent) 40%, transparent); }

/* --------------------------------------------------------------------------
   Forms
   -------------------------------------------------------------------------- */
.form { padding: 1.25rem; display: flex; flex-direction: column; gap: 1rem; }
.field { display: flex; flex-direction: column; gap: .35rem; }
.field--grow { flex: 1; }
.field__label { font-size: .82rem; font-weight: 580; color: var(--fg-muted);
  display: flex; align-items: baseline; gap: .5rem; }
.field__note { font-weight: 400; color: var(--fg-subtle); font-size: .78rem; }

.input, .textarea {
  width: 100%; font: inherit; color: var(--fg);
  background: var(--surface);
  border: 1px solid var(--border-strong); border-radius: var(--radius-sm);
  padding: .5rem .7rem;
  transition: border-color .12s ease, box-shadow .12s ease;
}
.input::placeholder, .textarea::placeholder { color: var(--fg-subtle); }
.input:focus, .textarea:focus {
  outline: none; border-color: var(--accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
}
.input--lg { padding: .65rem .85rem; font-size: 1rem; }
.textarea {
  font-family: var(--mono); font-size: .88rem; line-height: 1.65;
  resize: vertical; min-height: 24rem; tab-size: 2;
}

.btn {
  display: inline-flex; align-items: center; justify-content: center;
  gap: .4rem; font: inherit; font-weight: 550; font-size: .9rem;
  padding: .45rem .9rem; border-radius: var(--radius-sm);
  background: var(--accent); color: var(--accent-fg);
  border: 1px solid transparent; cursor: pointer;
  transition: background .12s ease, border-color .12s ease, color .12s ease;
}
.btn:hover { background: var(--accent-hover); text-decoration: none; color: var(--accent-fg); }
.btn--sm { font-size: .84rem; padding: .34rem .7rem; }
.btn--block { width: 100%; }
.btn--ghost {
  background: transparent; color: var(--fg-muted); border-color: var(--border-strong);
}
.btn--ghost:hover { background: var(--surface-sunk); color: var(--fg); }

/* --------------------------------------------------------------------------
   Editor
   -------------------------------------------------------------------------- */
.editor { display: flex; flex-direction: column; gap: 1.1rem; }
.editor__head {
  display: flex; align-items: flex-end; gap: 1rem; flex-wrap: wrap;
  padding-bottom: 1rem; border-bottom: 1px solid var(--border);
}
.editor__ident { display: flex; flex-direction: column; gap: .2rem; min-width: 0; }
.editor__title { font-size: 1.35rem; margin: 0; letter-spacing: -.02em; font-weight: 640; }
.editor__actions { display: flex; gap: .45rem; margin-left: auto; }

/* --------------------------------------------------------------------------
   Search
   -------------------------------------------------------------------------- */
.search-head { margin-bottom: 1.5rem; }
.search-head h1 { margin: 0 0 .9rem; font-size: 1.5rem; letter-spacing: -.024em; }
.search-form { display: flex; gap: .5rem; }
.search-form .input { flex: 1; }

.results { list-style: none; padding: 0; margin: 1rem 0 0; }
.results__item { padding: .9rem 0; border-bottom: 1px solid var(--border); }
.results__item:last-child { border-bottom: 0; }
.results__link { font-weight: 570; letter-spacing: -.01em; }
.results__item .path { display: block; margin-top: .1rem; }
.results__snippet { margin: .45rem 0 0; color: var(--fg-muted); font-size: .9rem; }
mark { background: var(--mark-bg); color: var(--mark-fg); padding: 0 .15em; border-radius: 3px; }

/* --------------------------------------------------------------------------
   Timeline (history)
   -------------------------------------------------------------------------- */
.timeline { list-style: none; margin: 0; padding: 0 0 0 1.1rem; position: relative; }
.timeline::before {
  content: ""; position: absolute; left: 3px; top: .55rem; bottom: .55rem;
  width: 1px; background: var(--border);
}
.timeline__item { position: relative; padding: .55rem 0 .55rem .5rem; }
.timeline__marker {
  position: absolute; left: -1.1rem; top: .95rem;
  width: 7px; height: 7px; border-radius: 50%;
  background: var(--border-strong); outline: 3px solid var(--bg);
}
.timeline__msg { font-weight: 520; }
.timeline__meta { display: flex; align-items: center; gap: .45rem;
  flex-wrap: wrap; margin-top: .2rem; font-size: .8rem; color: var(--fg-subtle); }
.oid { color: var(--fg-subtle); }

/* --------------------------------------------------------------------------
   Actor list + definition list
   -------------------------------------------------------------------------- */
.actor-list { list-style: none; margin: 0; padding: 0; }
.actor-list__item {
  display: flex; align-items: center; gap: .9rem; flex-wrap: wrap;
  padding: .7rem .6rem; margin: 0 -.6rem;
  border-bottom: 1px solid var(--border); border-radius: var(--radius-sm);
}
.actor-list__item:hover { background: var(--surface-sunk); }
.actor-list__item:last-child { border-bottom: 0; }
.actor-list__name { color: var(--fg-muted); font-size: .88rem; }
.actor-list__status { margin-left: auto; }

.deflist { display: grid; grid-template-columns: auto 1fr; gap: .55rem 1.25rem; margin: 0; }
.deflist dt { color: var(--fg-subtle); font-size: .82rem; }
.deflist dd { margin: 0; }

/* --------------------------------------------------------------------------
   Empty states + alerts
   -------------------------------------------------------------------------- */
.empty {
  padding: 2.25rem 1.5rem; text-align: center;
  border: 1px dashed var(--border-strong); border-radius: var(--radius);
  background: color-mix(in srgb, var(--surface) 60%, transparent);
}
.empty--tall { padding: 3.5rem 1.5rem; }
.empty__title { font-weight: 580; margin: 0 0 .35rem; }
.empty__body { color: var(--fg-muted); margin: 0 0 1rem; font-size: .92rem; }
.empty__body:last-child { margin-bottom: 0; }

.alert {
  padding: .6rem .85rem; border-radius: var(--radius-sm);
  font-size: .9rem; margin-bottom: 1rem; border: 1px solid transparent;
}
.alert--error { background: var(--danger-bg); color: var(--danger-fg);
  border-color: var(--danger-border); }

/* --------------------------------------------------------------------------
   Narrow screens
   -------------------------------------------------------------------------- */
@media (max-width: 34rem) {
  .topbar { flex-wrap: wrap; gap: .5rem; }
  .nav { width: 100%; margin-left: 0; }
  .page-list__meta { width: 100%; }
  .editor__actions { margin-left: 0; width: 100%; }
  .editor__actions .btn { flex: 1; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::{SNIPPET_CLOSE, SNIPPET_OPEN};

    /// The regression guard for stored XSS via search results.
    #[test]
    fn snippet_keeps_marks_and_escapes_page_html() {
        let raw = format!("before {SNIPPET_OPEN}hit{SNIPPET_CLOSE} <img src=x onerror=alert(1)>");
        let out = render_snippet(&raw).into_string();
        assert!(out.contains("<mark>hit</mark>"), "marks survive: {out}");
        assert!(!out.contains("<img"), "raw html must not survive: {out}");
        assert!(out.contains("&lt;img"), "and must appear escaped: {out}");
    }

    #[test]
    fn snippet_without_marks_is_escaped_text() {
        let out = render_snippet("<script>alert(1)</script>").into_string();
        assert!(!out.contains("<script"), "{out}");
        assert!(out.contains("&lt;script&gt;"), "{out}");
    }

    /// A sentinel with no closing partner must not become an escape hatch.
    #[test]
    fn unterminated_mark_still_escapes() {
        let raw = format!("a {SNIPPET_OPEN}<b>unclosed");
        let out = render_snippet(&raw).into_string();
        assert!(out.contains("<mark>"), "{out}");
        assert!(!out.contains("<b>"), "{out}");
    }

    #[test]
    fn rendered_markdown_does_not_pass_through_raw_html() {
        let out = render_markdown("hello <img src=x onerror=alert(1)>").into_string();
        assert!(!out.contains("<img"), "{out}");
    }

    /// An actor handle is user-supplied and lands in a class attribute via
    /// the type. Both must be escaped rather than able to close the tag.
    #[test]
    fn actor_chip_escapes_handle_and_type() {
        let out = actor_chip("</span><script>x</script>", Some("\" onmouseover=\"x")).into_string();
        assert!(!out.contains("<script"), "{out}");
        assert!(!out.contains("onmouseover=\"x\""), "{out}");
    }

    #[test]
    fn commit_author_splits_type_from_handle() {
        let out = commit_author_chip("agent:researcher").into_string();
        assert!(out.contains("actor--agent"), "{out}");
        assert!(out.contains("researcher"), "{out}");

        // A commit from outside wikikiki has no type prefix to split.
        let plain = commit_author_chip("Some Person").into_string();
        assert!(plain.contains("actor--unknown"), "{plain}");
    }

    #[test]
    fn relative_time_buckets() {
        let now = chrono::Utc::now().timestamp();
        assert_eq!(rel_time(now), "just now");
        assert_eq!(rel_time(now - 600), "10m ago");
        assert_eq!(rel_time(now - 7_200), "2h ago");
        assert_eq!(rel_time(now - 3 * 86_400), "3d ago");
        // Older than a month falls back to an absolute stamp.
        assert!(rel_time(now - 200 * 86_400).contains('-'));
    }

    /// A clock-skewed writer must not render as "just now" — the ordering of
    /// the guard chain is what makes that arm reachable at all.
    #[test]
    fn future_timestamps_show_an_absolute_stamp() {
        let now = chrono::Utc::now().timestamp();
        let out = rel_time(now + 7_200);
        assert!(out.contains('-'), "expected a date, got {out}");
        // Small skew is still friendly.
        assert_eq!(rel_time(now + 5), "just now");
    }

    #[test]
    fn leading_title_heading_is_dropped_from_the_body() {
        let c = "# Welcome\n\nbody text\n";
        assert_eq!(body_after_title(c, "Welcome"), "body text\n");
    }

    #[test]
    fn a_different_first_heading_is_kept() {
        let c = "# Something else\n\nbody\n";
        assert_eq!(body_after_title(c, "Welcome"), c);
        // No heading at all: title came from the path, body is untouched.
        let c2 = "just prose\n";
        assert_eq!(body_after_title(c2, "notes"), c2);
    }

    #[test]
    fn title_only_page_leaves_an_empty_body() {
        assert_eq!(body_after_title("# Welcome", "Welcome"), "");
        assert_eq!(body_after_title("# Welcome\n", "Welcome"), "");
    }

    #[test]
    fn layer_chip_uses_the_layer_as_a_modifier() {
        assert!(layer_chip("semantic").into_string().contains("chip--semantic"));
    }
}

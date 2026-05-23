//! Maud-based HTML templates. Server-rendered, minimal CSS, no SPA.
//!
//! Editing is a plain `<textarea>` over raw markdown — no WYSIWYG, per
//! DESIGN.md §10/§14.

use comrak::{markdown_to_html, ComrakOptions};
use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::auth::Auth;
use crate::db::queries::Actor;
use crate::git::CommitInfo;
use crate::pages::{Page, PageMeta, SearchHit};

pub fn render_markdown(src: &str) -> Markup {
    let mut opts = ComrakOptions::default();
    opts.extension.strikethrough = true;
    opts.extension.table = true;
    opts.extension.autolink = true;
    opts.extension.tasklist = true;
    opts.render.unsafe_ = false;
    PreEscaped(markdown_to_html(src, &opts))
}

fn layout(title: &str, auth: Option<&Auth>, body: Markup) -> Markup {
    let title_full = format!("{title} · wikikiki");
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title_full) }
                style { (PreEscaped(STYLES)) }
            }
            body {
                header.topbar {
                    a.brand href="/" { "wikikiki" }
                    nav {
                        a href="/" { "home" }
                        a href="/search" { "search" }
                        a href="/actors" { "actors" }
                        @if let Some(a) = auth {
                            a href="/me" { (a.actor.handle) " (" (a.actor.actor_type) ")" }
                            form method="post" action="/logout" style="display:inline" {
                                button type="submit" class="linkish" { "logout" }
                            }
                        } @else {
                            a href="/login" { "log in" }
                        }
                    }
                }
                main { (body) }
                footer.footer {
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

pub fn home(auth: Option<&Auth>, recent: &[PageMeta]) -> Markup {
    let body = html! {
        h1 { "wikikiki" }
        p.lede { "A wiki that is the live memory of a system of actors." }
        section {
            h2 { "Recent pages" }
            @if recent.is_empty() {
                p.muted {
                    "No pages yet. "
                    a href=(wiki_edit_url("welcome")) { "Create the first one." }
                }
            } @else {
                ul.page-list {
                    @for p in recent {
                        li {
                            a href=(wiki_url(&p.path)) { (p.title) }
                            span.path { " " (p.path) }
                            span.layer { " · " (p.layer) }
                        }
                    }
                }
            }
        }
    };
    layout("home", auth, body)
}

pub fn login(auth: Option<&Auth>, error: Option<&str>) -> Markup {
    let body = html! {
        h1 { "Log in" }
        @if let Some(e) = error {
            div.error { (e) }
        }
        form method="post" action="/login" class="login" {
            label { "Handle"   input type="text"     name="handle"   autocomplete="username" required; }
            label { "Password" input type="password" name="password" autocomplete="current-password" required; }
            button type="submit" { "Log in" }
        }
        p.muted { "Agents authenticate with bearer tokens against the REST API; tokens are issued by an admin via the CLI." }
    };
    layout("log in", auth, body)
}

pub fn view_page(auth: Option<&Auth>, page: &Page) -> Markup {
    let edit_url = wiki_edit_url(&page.meta.path);
    let history_url = wiki_history_url(&page.meta.path);
    let source_url = wiki_source_url(&page.meta.path);
    let body = html! {
        article {
            div.page-bar {
                span.layer { (page.meta.layer) }
                span.path { (page.meta.path) }
                span.spacer {}
                a href=(edit_url) { "edit" }
                " "
                a href=(history_url) { "history" }
                " "
                a href=(source_url) { "source" }
            }
            h1 { (page.meta.title) }
            div.rendered { (render_markdown(&page.content)) }
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
    let cancel_url = wiki_url(page_path);
    let body = html! {
        h1 { "Edit " (title) }
        form method="post" action=(post_url) class="edit" {
            input type="hidden" name="layer" value=(layer);
            label {
                "Summary"
                input type="text" name="summary" placeholder="short message — what changed and why";
            }
            label.editor-label {
                "Markdown source"
                textarea name="content" rows="24" autofocus { (content) }
            }
            div.buttons {
                button type="submit" { "Save" }
                a.cancel href=(cancel_url) { "Cancel" }
            }
        }
        p.muted { "No WYSIWYG. The text in the box above is the text that will be saved." }
    };
    layout("edit", auth, body)
}

pub fn missing_page(auth: Option<&Auth>, page_path: &str) -> Markup {
    let edit_url = wiki_edit_url(page_path);
    let body = html! {
        h1 { "Page not found" }
        p { code { (page_path) } " does not exist yet." }
        @if auth.is_some() {
            p { a.button href=(edit_url) { "Create it" } }
        } @else {
            p.muted { a href="/login" { "Log in" } " to create this page." }
        }
    };
    layout("not found", auth, body)
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
        h1 { "History · " (title) }
        p { a href=(back_url) { "back to page" } }
        @if commits.is_empty() {
            p.muted { "No commits found for this page." }
        } @else {
            table.history {
                thead { tr { th { "When" } th { "Who" } th { "Message" } th { "Commit" } } }
                tbody {
                    @for c in commits {
                        tr {
                            td { (fmt_time(c.time)) }
                            td { (c.author_name) }
                            td { (c.message) }
                            td.commit { (short_oid(&c.oid)) }
                        }
                    }
                }
            }
        }
    };
    layout("history", auth, body)
}

pub fn search(auth: Option<&Auth>, query: &str, hits: &[SearchHit]) -> Markup {
    let body = html! {
        h1 { "Search" }
        form method="get" action="/search" class="search-form" {
            input type="text" name="q" value=(query) placeholder="search the wiki…" autofocus;
            button type="submit" { "Search" }
        }
        @if query.is_empty() {
            p.muted { "Enter a query above. Prefix-matches each token." }
        } @else if hits.is_empty() {
            p.muted { "No matches for " code { (query) } "." }
        } @else {
            ul.search-results {
                @for h in hits {
                    li {
                        a href=(wiki_url(&h.path)) { (h.title) }
                        span.path { " " (h.path) }
                        div.snippet { (PreEscaped(h.snippet.clone())) }
                    }
                }
            }
        }
    };
    layout("search", auth, body)
}

pub fn me(auth: &Auth) -> Markup {
    let body = html! {
        h1 { "You are " (auth.actor.handle) }
        p {
            "Type: " strong { (auth.actor.actor_type) }
            @if auth.actor.is_admin {
                " · " span.badge.admin { "admin" }
            }
        }
        p { "Name: " (auth.actor.name) }
        p.muted { "Token management and profile-page editing land in later stages." }
    };
    layout("me", Some(auth), body)
}

pub fn actors_list(auth: Option<&Auth>, actors: &[Actor]) -> Markup {
    let body = html! {
        h1 { "Actors" }
        table.actors {
            thead { tr { th { "Handle" } th { "Type" } th { "Name" } th { "Status" } } }
            tbody {
                @for a in actors {
                    tr {
                        td { (a.handle) }
                        td { span.badge { (a.actor_type) } }
                        td { (a.name) }
                        td {
                            @if !a.is_active { span.badge.revoked { "revoked" } }
                            @else if a.is_admin { span.badge.admin { "admin" } }
                            @else { span.muted { "active" } }
                        }
                    }
                }
            }
        }
    };
    layout("actors", auth, body)
}

fn short_oid(oid: &str) -> String {
    oid.chars().take(8).collect()
}

fn fmt_time(unix_ts: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix_ts, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| unix_ts.to_string())
}

const STYLES: &str = r#"
:root {
  --fg: #1a1a1a;
  --muted: #6b6b6b;
  --bg: #fbfbfa;
  --accent: #3b6cb7;
  --border: #e3e3e0;
  --code-bg: #f3f3ef;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  font: 15px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
  color: var(--fg);
  background: var(--bg);
}
.topbar {
  display: flex; align-items: center; gap: 1.5rem;
  padding: .75rem 1.5rem;
  border-bottom: 1px solid var(--border);
  background: white;
}
.topbar .brand { font-weight: 700; font-size: 1.05rem; text-decoration: none; color: var(--fg); }
.topbar nav { display: flex; gap: 1rem; margin-left: auto; align-items: center; }
.topbar nav a { color: var(--accent); text-decoration: none; }
.topbar nav a:hover { text-decoration: underline; }
.linkish {
  background: none; border: none; color: var(--accent); cursor: pointer; padding: 0; font: inherit;
}
main { max-width: 820px; margin: 0 auto; padding: 1.5rem; }
h1 { margin-top: 0.5rem; }
.lede { color: var(--muted); }
.page-list { padding-left: 1rem; }
.page-list li { margin: .25rem 0; }
.page-list .path { color: var(--muted); font-size: .85em; font-family: monospace; }
.page-list .layer { color: var(--muted); font-size: .85em; }
.page-bar {
  display: flex; gap: .75rem; align-items: center;
  font-size: .85em; color: var(--muted);
  margin-bottom: 1rem;
}
.page-bar .spacer { flex: 1; }
.page-bar .layer { background: var(--code-bg); padding: 2px 6px; border-radius: 4px; }
.page-bar .path { font-family: monospace; }
.rendered { line-height: 1.6; }
.rendered code { background: var(--code-bg); padding: 1px 4px; border-radius: 3px; }
.rendered pre { background: var(--code-bg); padding: .75rem; border-radius: 6px; overflow-x: auto; }
.rendered pre code { background: none; padding: 0; }
.rendered table { border-collapse: collapse; }
.rendered th, .rendered td { border: 1px solid var(--border); padding: 4px 8px; }
form.login, form.edit { display: flex; flex-direction: column; gap: .6rem; max-width: 640px; }
form.edit label.editor-label { display: flex; flex-direction: column; }
form.edit textarea {
  width: 100%; font: 14px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
  border: 1px solid var(--border); border-radius: 4px; padding: .5rem;
  background: white;
}
form input[type=text], form input[type=password] {
  border: 1px solid var(--border); border-radius: 4px; padding: .4rem .55rem;
  font: inherit; background: white;
}
button {
  border: 1px solid var(--accent); background: var(--accent); color: white;
  padding: .45rem .85rem; border-radius: 4px; cursor: pointer; font: inherit;
}
button:hover { filter: brightness(1.05); }
.buttons { display: flex; gap: .5rem; align-items: center; }
.buttons .cancel { color: var(--muted); }
.muted { color: var(--muted); }
.error {
  background: #fbe6e6; color: #9b1d1d; padding: .5rem .75rem; border-radius: 4px;
  border: 1px solid #f3c2c2;
}
table.history, table.actors { border-collapse: collapse; width: 100%; }
table.history th, table.history td, table.actors th, table.actors td {
  border-bottom: 1px solid var(--border); padding: .35rem .5rem; text-align: left;
  font-size: .9em;
}
table.history .commit { font-family: monospace; color: var(--muted); }
.search-form { display: flex; gap: .5rem; }
.search-form input { flex: 1; }
.search-results { list-style: none; padding: 0; }
.search-results li { margin-bottom: .8rem; }
.search-results .snippet { color: var(--muted); font-size: .9em; }
.search-results mark { background: #ffeaa7; padding: 0 2px; border-radius: 2px; }
.badge {
  display: inline-block; padding: 1px 6px; border-radius: 999px;
  background: var(--code-bg); color: var(--muted); font-size: .75em;
}
.badge.admin { background: #fde7c3; color: #8a4b00; }
.badge.revoked { background: #fbe6e6; color: #9b1d1d; }
.footer { text-align: center; color: var(--muted); padding: 2rem 1rem; font-size: .8em; }
"#;

# Wikikiki

> A wiki that is the live memory of a system of actors — human, agent, and machine — observable and editable in real time.

See [DESIGN.md](DESIGN.md) for the full architecture document.

## Status

**Stage 1** — markdown wiki with git backing, full access foundation, two actor types (`human` via cookie session, `agent` via API token), attribution, FTS5 search.

Stages 2–4 (live working memory, distillation, hardening) are deferred per [§17](DESIGN.md#17-staged-delivery-plan).

See [ROADMAP.md](ROADMAP.md) for the proposed delivery sequence and
[the handoff pilot](docs/HANDOFF_PILOT.md) for five real-work trials using the
current page API.

## Write consistency

Page writes through one shared server instance serialize file content, Git
commits, metadata, edit history, and FTS updates in the same order. Unknown or
inactive layers and invalid actor identities are rejected before content changes.
An accepted write continues if its caller stops waiting, and normal server
shutdown drains these writes before stopping the runtime.

This is not a transaction across Git and SQLite. A process crash or a storage
error after Git succeeds can still require reconciliation. Reads during a save
are not guaranteed to return a snapshot across files and database metadata.
Use one server per content repository and clone its shared `Repo` handle for
library callers. Separate processes or separately opened handles do not share
the writer lock. API writes still replace the whole page without revision checks
or idempotency keys.

Run `cargo test --locked` for the complete test suite. The
`page_consistency` integration target checks the full persistence pipeline,
including concurrent writers, rejected input, and cancelled callers.

## Quickstart

```sh
# 1. Build
cargo build --release

# 2. Configure (optional — defaults are fine)
cp wikikiki.example.toml wikikiki.toml

# 3. Bootstrap the first admin (creates the wiki + DB + git repo)
./target/release/wikikiki bootstrap --handle alice --password 'change-me'

# 4. Run the server
./target/release/wikikiki serve

# 5. Open http://localhost:8090 and log in as alice.
```

## Issuing an agent token

```sh
./target/release/wikikiki issue-token --handle research-bot --type agent --label "first agent"
# prints the plaintext token once; store it somewhere safe.
```

Use it from an agent:

```sh
curl -H "Authorization: Bearer <token>" http://localhost:8090/api/pages/welcome
curl -H "Authorization: Bearer <token>" -X PUT \
     -H "Content-Type: text/markdown" \
     --data 'hello from an agent' \
     http://localhost:8090/api/pages/notes/from-agent
```

## Layout

```
src/
├── main.rs        CLI entrypoint (serve, bootstrap, issue-token)
├── lib.rs
├── config/        TOML loader with env overrides
├── db/            sqlx pool, migrations runner, queries
├── git/           gix wrapper for the content repo
├── web/           axum router, maud templates, auth middleware
└── actor/         REST API surface for agents
migrations/        sqlx SQL migrations (lookups seeded here)
content/           the wiki itself (markdown files; path is configurable)
```

---

A part of the TAGENT
Private Edge Office Stack | An AI Agent Platform Project by AI VISIONS

https://www.linkedin.com/feed/update/urn:li:activity:7447711407745200129/

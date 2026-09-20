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
the writer lock. A write still replaces the whole page rather than merging, and
there are no idempotency keys: a retried request that already succeeded writes
again.

## Conditional writes

A write can name the revision it is modifying, so an actor cannot erase a
change it never saw. This matters most for agents, which read a page, reason
for a while, and write back a conclusion — without it, whoever is slowest wins.

`GET` returns the current revision as an `ETag`, and `PUT` honours the
matching conditional headers:

| Request | Behaviour |
|---|---|
| `If-Match: "<revision>"` | Writes only if the page is still at that revision, otherwise `412` |
| `If-Match: *` | Writes only if the page exists |
| `If-None-Match: *` | Creates only if the page does not exist |
| *(no condition)* | Last writer wins, unchanged |

```sh
etag=$(curl -sI -H "Authorization: Bearer $TOKEN" \
        "$BASE/api/pages/notes/topic" | grep -i '^etag:' | cut -d' ' -f2- | tr -d '\r')

curl -X PUT -H "Authorization: Bearer $TOKEN" -H "If-Match: $etag" \
     --data 'updated body' "$BASE/api/pages/notes/topic"
```

The revision is a hash of the page content, so it means "the version that says
*this*" and a client can verify it from bytes it already holds. The condition
is evaluated inside the writer lock, immediately before the Git commit —
checking it any earlier would leave the window it exists to close.

Omitting the headers keeps the previous behaviour, so existing clients
including `scripts/wikikiki-sync.sh` are unaffected.

Run `cargo test --locked` for the complete test suite. The
`page_consistency` integration target checks the full persistence pipeline,
including concurrent writers, rejected input, and cancelled callers;
`end_to_end` covers the conditional-write surface.

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
├── throttle.rs    coalesces last-seen/last-used writes off the request path
└── actor/         REST API surface for agents
migrations/        sqlx SQL migrations (lookups seeded here)
content/           the wiki itself (markdown files; path is configurable)
```

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The most useful contribution right now
is not code: it is [running the handoff pilot](docs/HANDOFF_PILOT.md) and
reporting what happened, including if nothing did.

## Licence

MIT or Apache-2.0, at your option — [LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE).

---

A part of the TAGENT
Private Edge Office Stack | An AI Agent Platform Project by AI VISIONS

https://www.linkedin.com/feed/update/urn:li:activity:7447711407745200129/

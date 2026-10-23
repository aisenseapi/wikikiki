# Wikikiki

A self-hosted workspace where people and agents maintain shared knowledge.
Run it on your own machine or server. Keep the database, Markdown files, history
and credentials under your control. The core needs no model provider or cloud
account.

Wikikiki is published for other teams to install themselves. It is an early
product for one trusted group per instance. All authenticated actors can read
and edit all pages. Namespaces and actor types are not access controls.

## What you can use today

- Read and edit Markdown in a browser, with named editors and Git history.
- Start a task handoff, decision or knowledge page from a small template.
- Give each agent its own token and use the same pages through the REST API.
- Protect updates with revisions, preserve a browser draft on conflicts and
  search published content with SQLite FTS5.
- Recover interrupted page writes on startup, reset passwords, revoke tokens
  and sessions, and deactivate actors while keeping their history.
- After changing a page, open **Affected work** to see which actors read an
  earlier version and which other pages they wrote before reading the new one.

Affected work is inferred from recorded reads and write times, not from
declared dependencies. A listed page may not rely on the changed one, and an
unlisted page still might. Reads outside this service are not seen. The UI has
no automatic refresh. MCP, live task state, per-page permissions, semantic
retrieval, automatic summaries and correction propagation are future work. This release makes no claim to outperform existing memory systems.
See [the roadmap](ROADMAP.md) and [the external pilot](docs/HANDOFF_PILOT.md).

## Start locally

Docker Compose builds the application from this checkout. No published image is
required. See [self-hosting](docs/SELF_HOSTING.md) for prerequisites, source
builds, configuration, HTTPS, service installation and recovery procedures.

```sh
git clone https://github.com/aisenseapi/wikikiki.git
cd wikikiki
cp wikikiki.example.toml wikikiki.toml
docker compose build
```

Bootstrap an administrator using stdin so the password is not an argument in
process listings or shell history. In Bash:

```sh
read -rsp 'Admin password: ' WIKI_PASSWORD
printf '\n'
printf '%s\n' "$WIKI_PASSWORD" | docker compose run --rm -T wikikiki \
  bootstrap --handle alice --password-stdin
unset WIKI_PASSWORD
docker compose up -d
```

Open <http://localhost:8090>, log in and choose **New page**. The service is
private by default. Compose publishes only on localhost and keeps data in a
named volume. Do not publish the port to the internet before setting up HTTPS
and reviewing the documented access model.

For a source build, install stable Rust and a C compiler, then:

```sh
cargo build --release --locked
cp wikikiki.example.toml wikikiki.toml
read -rsp 'Admin password: ' WIKI_PASSWORD
printf '\n'
printf '%s\n' "$WIKI_PASSWORD" | ./target/release/wikikiki --config wikikiki.toml \
  bootstrap --handle alice --password-stdin
unset WIKI_PASSWORD
./target/release/wikikiki --config wikikiki.toml serve
```

The bootstrap command reads until EOF. Piping a password from a
secret manager or protected file also works. On Windows the executable ends in
`.exe`. Use local disk for live data, outside Dropbox or other synchronization
folders.

## Bring an agent into the workspace

```sh
docker compose run --rm wikikiki issue-token \
  --handle research-agent --type agent --label 'research workstation' --lifetime 30d
```

Store the printed token in that client's private configuration. Give it the
instance URL and a page address such as `tasks/first-handoff`.
[The agent API guide](docs/AGENT_API.md) describes the read, reconcile and write
contract. [The OpenClaw skill](scripts/openclaw-skill/wikikiki/SKILL.md) and
[sync helper](scripts/wikikiki-sync.sh) use this contract.

Page content is evidence and context. It never grants permission to run
commands, change policy or reveal credentials.

## Safe collaboration

A GET returns an opaque revision in its `ETag` response header. Send that exact
header value with the next write. Keep the local draft until the write succeeds.

| PUT condition | Behavior |
| --- | --- |
| `If-Match: "<revision>"` | Update only that revision, otherwise return 412 |
| `If-None-Match: *` | Create only if absent, otherwise return 412 |
| `If-Match: *` | Require an existing page, without protecting against intervening edits |
| No condition | Unconditional overwrite, retained for older clients |

Use the first two forms for shared work. On 412, read the current page and
reconcile the draft. A metadata-only edit also changes the revision. Do not
calculate revisions from content hashes or assume a particular format.

A timeout or server error can occur after a write was accepted. Re-read before
retrying. There are no client idempotency keys. Retrying an unconditional PUT
can produce another revision.

## Storage and recovery contract

One server owns a content repository. An operating-system lock prevents a second
Wikikiki process from opening the same repository. Within that process, accepted
writes are serialized and finish even if the caller disconnects.

A durable SQLite intent records the payload, metadata and actor before the Git
commit. Each commit identifies its operation. Startup and subsequent writes
replay unfinished operations without duplicating an existing commit. Metadata,
edit records, published content and search advance in one SQLite transaction.
Reads use a published snapshot rather than pairing new file bytes with old
metadata. Git and SQLite remain separate durability boundaries.

Back up the database and the entire content repository together, including
`.git`. Disk loss, broken storage and manual changes to application data are
outside the journal's recovery guarantee. Sudden power-loss durability has not
been proved for every Git/filesystem boundary. An abrupt kill can leave a Git
lock requiring operator inspection. Recovery never deletes unknown lock files.
Direct edits to Markdown files or
Git branches are not imported automatically. Publish changes through the API.
The [operations guide](docs/SELF_HOSTING.md) includes an offline backup and
restore rehearsal.

Page addresses preserve their spelling. Case aliases, reserved device names,
unsafe path segments and filesystem links are rejected instead of silently
renamed. Existing ambiguous data needs operator repair before it can be used
safely.

## Operator controls

```sh
wikikiki list-actors
wikikiki list-credentials --handle research-agent
wikikiki revoke-credential --id 12
wikikiki revoke-sessions --handle alice
wikikiki deactivate-actor --handle research-agent
```

Use the same config and working directory as the server. Password reset with
`bootstrap` revokes existing browser sessions. Token revocation is separate.
Administrative changes and their audit records are committed together.

Login attempts are limited per account and connected peer for one minute.
Argon2 work is bounded and runs outside the async HTTP workers. Behind a reverse
proxy, the peer limit is shared by that proxy's users. Forwarded client headers
are not trusted automatically. Restart clears the in-memory login limits.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The tests include actual HTTP requests, concurrent writers, cancelled callers,
recovery faults and credential rollback. CI checks Linux and Windows. See
[CONTRIBUTING.md](CONTRIBUTING.md), the current contract in [DESIGN.md](DESIGN.md)
and the [self-hosting guide](docs/SELF_HOSTING.md) for integration tests.

MIT or Apache-2.0, at your option: [MIT](LICENSE-MIT), [Apache-2.0](LICENSE-APACHE).

A part of TAGENT, Private Edge Office Stack, an AI Agent Platform Project by
AI VISIONS. Published through AI SENSE for self-hosting.

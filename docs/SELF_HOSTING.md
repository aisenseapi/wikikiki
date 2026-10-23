# Self-hosting Wikikiki

Wikikiki is software you run on your own infrastructure. The installation below
uses one server process, one SQLite database and one content directory containing
Markdown and Git history. No model provider or external database is required.
AI SENSE does not operate an instance as part of this installation.

Use local disk for the data directory. Do not run multiple server instances against
the same database and Git repository, or place the live directory in Dropbox, a
network share or another file synchronization service. Backups can be copied elsewhere.

The defaults require authentication for reads and writes. All authenticated actors
share the workspace and can edit its pages. Page paths are organizational conventions,
not access controls. Use separate instances when groups need separate access.

## Docker Compose

The repository supplies a Dockerfile, not a published image. It builds with the
[official Rust image](https://hub.docker.com/_/rust) and runs as UID/GID `10001` in
Debian. CI defines build and restore checks on Linux x86-64. Other architectures
are not covered by those checks. The commands below use Bash and Docker Compose v2.

From the repository root:

```bash
cp wikikiki.example.toml wikikiki.toml
docker compose build --pull
read -r -s -p 'Admin password: ' WK_ADMIN_PASSWORD
printf '\n'
printf '%s\n' "$WK_ADMIN_PASSWORD" | docker compose run --rm -T wikikiki bootstrap --handle admin --password-stdin
unset WK_ADMIN_PASSWORD
docker compose up -d --wait --wait-timeout 120
docker compose ps
```

Open `http://localhost:8090/login` and sign in as `admin`. The host port binds to
`127.0.0.1` only. The container listens on `0.0.0.0:8090` inside its own network.
`GET /healthz` returns a public health result without page contents and checks the
database. It does not replace a write/read smoke test or a restore drill.

Compose mounts `wikikiki.toml` read-only and stores all application data in the
named volume `wikikiki-data` at `/data`. It refuses to create a missing config
mount. `docker compose down` keeps the volume. Do not use `down --volumes` for an
instance whose data you want to keep.

To choose another host port or volume, set `WIKIKIKI_PORT` or `WIKIKIKI_VOLUME`
before running Compose. Keep those values in your deployment's private `.env` file
so later operations select the same instance. If the host port changes, update
`server.public_url` in `wikikiki.toml` as well. Restart after config changes:

```bash
docker compose up -d --force-recreate --wait --wait-timeout 120
docker compose logs --tail 100 wikikiki
```

Use a named volume unless you need a host bind mount. A bind mount must already
be writable by UID/GID `10001`. Do not make the directory world-writable to fix
an ownership error. The container root filesystem is read-only, with writable
`/data` and a temporary `/tmp`.

## Source build and systemd

This path targets a Linux host with systemd. Install a current stable Rust
toolchain, Git and a C compiler. On Debian, `build-essential`, `pkg-config` and
`ca-certificates` provide the native build prerequisites. SQLite is compiled from
the bundled source. The server does not need a separate SQLite or Git daemon.

```bash
cargo build --locked --release --bin wikikiki
cargo test --locked
sudo useradd --system --user-group --home-dir /var/lib/wikikiki --shell /usr/sbin/nologin wikikiki
sudo install -d -o wikikiki -g wikikiki -m 0700 /var/lib/wikikiki
sudo install -d -o root -g wikikiki -m 0750 /etc/wikikiki
sudo install -m 0755 target/release/wikikiki /usr/local/bin/wikikiki
sudo install -o root -g wikikiki -m 0640 wikikiki.example.toml /etc/wikikiki/wikikiki.toml
sudo install -m 0644 scripts/wikikiki.service /etc/systemd/system/wikikiki.service
```

Run the user and directory creation steps only on first installation. Edit the
installed config, then bootstrap from the same working directory as the service:

```bash
read -r -s -p 'Admin password: ' WK_ADMIN_PASSWORD
printf '\n'
printf '%s\n' "$WK_ADMIN_PASSWORD" | sudo -u wikikiki sh -c 'cd /var/lib/wikikiki && exec /usr/local/bin/wikikiki --config /etc/wikikiki/wikikiki.toml bootstrap --handle admin --password-stdin'
unset WK_ADMIN_PASSWORD
sudo systemctl daemon-reload
sudo systemctl enable --now wikikiki
sudo systemctl status wikikiki
curl --fail http://127.0.0.1:8090/healthz
```

The supplied unit runs as `wikikiki`, writes only under `/var/lib/wikikiki`, and
waits up to 60 seconds for graceful shutdown. Use `journalctl -u wikikiki` for logs.
If you change the data paths, update the unit's `ReadWritePaths` and filesystem
permissions as well.

## Configuration and HTTPS

Environment overrides take precedence over TOML, then built-in defaults. With no
`--config`, the process reads `./wikikiki.toml` if present. An explicit missing
`--config` path is an error. Unknown TOML keys are rejected. Relative data paths
always resolve from the process working directory, including when the config
file lives elsewhere.

| Setting | Environment override | Default |
| --- | --- | --- |
| `paths.content_root` | `WIKIKIKI_CONTENT_ROOT` | `./content` |
| `paths.db_path` | `WIKIKIKI_DB_PATH` | `./wikikiki.db` |
| `paths.git_repo` | `WIKIKIKI_GIT_REPO` | `./content` |
| `server.bind` | `WIKIKIKI_BIND` | `127.0.0.1:8090` |
| `server.public_url` | `WIKIKIKI_PUBLIC_URL` | `http://localhost:8090` |
| `ui.allow_anonymous_read` | `WIKIKIKI_ALLOW_ANON_READ` | `false` |
| `ui.require_auth_for_write` | `WIKIKIKI_REQUIRE_AUTH_WRITE` | `true` |

Keep `content_root` and `git_repo` equal for this deployment. Keep the database
inside the durable directory too. Existing configs with anonymous reads enabled
remain public until you change them. A config with `require_auth_for_write = false`
is rejected because anonymous writes are unsupported.

For remote access, terminate HTTPS at a reverse proxy on the same host and keep
port 8090 bound to loopback. Set `server.public_url = "https://wiki.example.com"`
in the Wikikiki config, then restart. This must be the actual browser origin,
including its port, with no path prefix, query or credentials. Browser writes
with an `Origin` header must match it. Secure browser cookies follow an HTTPS
public URL automatically. Do not force `cookie_secure = false` on an HTTPS site.

For example, a host-installed Caddy can use this Caddyfile:

```caddyfile
wiki.example.com {
    reverse_proxy 127.0.0.1:8090
}
```

Replace the domain, configure its DNS and expose ports 80/443 to Caddy. Follow
[Caddy's HTTPS setup](https://caddyserver.com/docs/quick-starts/https) for certificate
issuance and service installation. If the proxy runs in another container,
`127.0.0.1` refers to that container, so configure its upstream network explicitly.
Forward `Authorization` headers and allow Markdown request bodies up to 4 MiB.
Do not expose credentials over remote plain HTTP.

## Actors, tokens and access removal

Each person should have their own account. Each agent or integration should have
its own actor and token. The actor attached to the token determines attribution.
Do not put passwords or tokens in Git, page contents, shell history or shared logs.

For a Compose installation:

```bash
wk() {
    docker compose exec wikikiki /usr/local/bin/wikikiki --config /etc/wikikiki/wikikiki.toml "$@"
}
wk issue-token --handle research-worker --type agent --label laptop --lifetime 30d
wk list-actors
wk list-credentials --handle research-worker
wk revoke-credential --id 12
wk revoke-sessions --handle admin
wk deactivate-actor --handle research-worker
```

Use the actual credential ID returned by `list-credentials`. Token issuance prints
the plaintext once. Store it in the integration's secret store. Rotation means
issuing a replacement, updating the client, confirming access, then revoking the
old credential. Revoking browser sessions is a separate operation. Deactivation
removes access while preserving attributed history.

For systemd, run the same subcommands through this prefix, replacing `list-actors`
with the needed command inside the quoted string:

```bash
sudo -u wikikiki sh -c 'cd /var/lib/wikikiki && exec /usr/local/bin/wikikiki --config /etc/wikikiki/wikikiki.toml list-actors'
```

`bootstrap` can reset a human admin's password. Use `--password-stdin`. A password
reset revokes that actor's browser sessions. Independently issued bearer tokens
remain active until revoked. CLI administration requires local access to the
instance's database and config.

## Offline backup and restore

A backup must include the database, any SQLite companion files, the entire content
directory with `.git`, and the deployment config. Git history alone does not contain
actors, credentials, sessions or the full application state. Store backups privately
because they contain page history and authentication data.

Stop the server and all CLI jobs that can modify the database. For Compose, from
the deployment directory with the same `.env` and volume selection:

```bash
set -euo pipefail
umask 077
backup_dir="$(mktemp -d "${TMPDIR:-/tmp}/wikikiki-backup.XXXXXXXX")"
docker compose stop
docker compose run --rm -T --no-deps --entrypoint tar wikikiki -C /data -czf - . > "$backup_dir/data.tar.gz"
cp wikikiki.toml compose.yaml "$backup_dir/"
if [ -f .env ]; then cp .env "$backup_dir/"; fi
docker image inspect wikikiki:local --format '{{.Id}}' > "$backup_dir/image-id.txt"
tar -tzf "$backup_dir/data.tar.gz" > /dev/null
docker compose up -d --wait --wait-timeout 120
printf 'Backup directory: %s\n' "$backup_dir"
```

Move the verified backup to durable backup storage. Keep the corresponding source
revision or image for rollback. Stop if any backup command fails. Listing the
archive checks readability, not application recovery.

Restore a trusted backup into a **new, empty** volume. Leave the original volume
intact. Use the config and image version that produced the backup, and preserve
your existing `WIKIKIKI_VOLUME` setting before changing it:

```bash
set -euo pipefail
docker compose stop
export WIKIKIKI_VOLUME=wikikiki-restored-20261023
docker volume create "$WIKIKIKI_VOLUME"
docker compose run --rm -T --no-deps --entrypoint tar wikikiki -C /data -xzf - < /path/to/backup/data.tar.gz
docker compose up -d --wait --wait-timeout 120
```

Choose a volume name that does not already exist. Docker initializes the empty
volume with the image's `/data` ownership, so extraction runs as UID `10001`.
After verification, persist the new volume selection in the deployment `.env`.
Verify login, token access, known page contents, search and history. Run the API
smoke check described in [AGENT_API.md](AGENT_API.md). Perform a restore drill before
depending on backups for important work.

For systemd, stop `wikikiki` and archive `/var/lib/wikikiki` plus `/etc/wikikiki`
with ownership and permissions preserved. Restore into a separate deployment or
empty directory while the service is stopped. Check ownership before starting it.
The supplied unit expects the data directory to belong to `wikikiki:wikikiki`.

## Upgrades and recovery

Record the running source revision and image ID. Read the target version's release
notes, stop writes, take an offline backup, and retain the previous binary/image.
Build the target source with `--locked`. For Compose, build the new `wikikiki:local`
image, then run `docker compose up -d --wait --wait-timeout 120`. For systemd, stop
the service, install the new binary and start it again. Do not overwrite the
operator's config with the example config during an upgrade.

Database migrations run when the application opens the database. Test the upgrade
against a restored backup first. A rollback may require both the old binary and
the pre-upgrade data. Do not assume an older binary can read a newer schema.

Check these compatibility changes when upgrading an existing installation:

- An explicit `allow_anonymous_read = true` stays public. When the setting is
  absent, the new default requires authentication.
- Unknown config keys, an unsupported Markdown renderer and
  `require_auth_for_write = false` now fail validation. Correct the config before
  restarting. Set `public_url` to the browser's actual origin.
- Existing ETags remain usable until the next accepted write. New writes use
  opaque operation revisions. Clients must preserve returned tags rather than
  compute a hash or derive a Git commit ID from them.
- Pages whose paths differ only in case can prevent migration. The application
  fails without silently deleting either page. Preserve a backup and inspect both
  versions before resolving their names and references.
- After a page is written under the new storage model, direct edits to its
  Markdown file do not change the published content. Use the UI or API so content,
  history and search remain consistent.

If startup recovery or a write fails, inspect server logs and preserve the full
data directory before attempting repairs. Re-read a page after a failed write
before retrying. Recovery checks cover process interruptions, but resilience to
every filesystem or power-loss failure has not been established. A leftover Git
lock can require operator inspection. Confirm that no writer is active before
repairing it, and never add an automatic lock-file deletion job.

#!/usr/bin/env bash
# wikikiki-sync — mirror an OpenClaw workspace into wikikiki.
#
# Watches a directory of markdown files; on each change, PUTs the file
# contents to wikikiki at agents/<actor>/<basename>. The agent it mirrors
# never knows wikikiki exists; the agent's bearer token gets used so writes
# are attributed correctly in the audit trail.
#
# Requirements: bash, curl, inotifywait (apt install inotify-tools).
#
# Usage:
#   export WIKIKIKI_BASE=http://127.0.0.1:8090
#   export WIKIKIKI_TOKEN=wk_xxx_yyy            # one issued for this agent
#   export WORKSPACE=~/.openclaw/workspace      # the dir to watch
#   export ACTOR_HANDLE=openclaw                # path namespace in wikikiki
#   bash scripts/wikikiki-sync.sh
#
# Or drop the env vars into /etc/default/wikikiki-sync and run via systemd
# (see scripts/wikikiki-sync.service).

set -euo pipefail

: "${WIKIKIKI_BASE:?set WIKIKIKI_BASE, e.g. http://127.0.0.1:8090}"
: "${WIKIKIKI_TOKEN:?set WIKIKIKI_TOKEN (issued via 'wikikiki issue-token')}"
: "${WORKSPACE:?set WORKSPACE, e.g. ~/.openclaw/workspace}"
: "${ACTOR_HANDLE:=openclaw}"

command -v inotifywait >/dev/null 2>&1 || {
  echo "inotifywait not found. Install with: sudo apt install inotify-tools" >&2
  exit 1
}

# Resolve to absolute path; reject if not a directory.
WORKSPACE="$(cd "$WORKSPACE" 2>/dev/null && pwd)" || {
  echo "WORKSPACE does not exist or is not a directory: $WORKSPACE" >&2
  exit 1
}

log() { printf '[wikikiki-sync] %s\n' "$*" >&2; }

# Lower-case + strip .md → page slug under agents/<handle>/.
# Example: MEMORY.md → agents/openclaw/memory
slug_for() {
  local f base lower
  f="$1"
  base="$(basename "$f" .md)"
  lower="$(printf '%s' "$base" | tr '[:upper:]' '[:lower:]')"
  printf 'agents/%s/%s' "$ACTOR_HANDLE" "$lower"
}

# Single PUT; idempotent — wikikiki's pages::write upserts on the path.
sync_file() {
  local file="$1" path body code
  [ -f "$file" ] || return 0
  case "$file" in *.md) ;; *) return 0 ;; esac

  path="$(slug_for "$file")"
  body="$(cat "$file")"
  code="$(curl -sS -o /dev/null -w '%{http_code}' \
    -X PUT \
    -H "Authorization: Bearer $WIKIKIKI_TOKEN" \
    -H "Content-Type: text/markdown" \
    --data-binary "$body" \
    "$WIKIKIKI_BASE/api/pages/$path" || echo "000")"

  if [ "$code" = "200" ]; then
    log "ok  $path  <-  $(basename "$file")"
  else
    log "FAIL $path  status=$code  file=$file"
  fi
}

# Initial pass: push every existing .md so wikikiki starts in sync.
log "initial sync from $WORKSPACE to $WIKIKIKI_BASE (actor=$ACTOR_HANDLE)"
for f in "$WORKSPACE"/*.md; do
  [ -e "$f" ] || continue
  sync_file "$f"
done

# Watch loop. close_write covers most editors; moved_to covers atomic-replace
# editors (vim with backup, sed -i). create catches new files.
log "watching $WORKSPACE for changes (Ctrl-C to stop)"
inotifywait -m -q \
  --format '%w%f %e' \
  -e close_write -e moved_to -e create \
  "$WORKSPACE" |
while read -r path event; do
  case "$path" in *.md) sync_file "$path" ;; esac
done

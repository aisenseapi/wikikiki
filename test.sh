#!/usr/bin/env bash
# wikikiki smoke test — write a page, read it back, search for it.
# Usage: edit TOKEN below (or export it), then: bash test.sh
set -euo pipefail

TOKEN="${TOKEN:-wk_xxx_yyy}"       # paste your token here, or `export TOKEN=...` before running
BASE="${BASE:-http://127.0.0.1:8090}"
HOST="$(hostname)"
PATH_="tests/linux-first"

if [ "$TOKEN" = "wk_xxx_yyy" ]; then
  echo "set TOKEN first (edit this script or run: export TOKEN=wk_...)" >&2
  exit 1
fi

echo "=== PUT $PATH_ ==="
curl -sS -X PUT \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: text/markdown" \
  --data "# Linux smoke test

agent writing from $HOST." \
  "$BASE/api/pages/$PATH_" -w "\nstatus %{http_code}\n"

echo
echo "=== GET $PATH_ ==="
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$BASE/api/pages/$PATH_" -w "\nstatus %{http_code}\n"

echo
echo "=== SEARCH 'smoke' ==="
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$BASE/api/search?q=smoke" -w "\nstatus %{http_code}\n"

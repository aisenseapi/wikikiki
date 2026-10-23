#!/usr/bin/env bash
# Creates a unique test page on the configured server and retains it for inspection.
# Requires Python 3.10+ and WIKIKIKI_TOKEN. WIKIKIKI_BASE defaults to localhost.
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$script_dir/scripts/tests/smoke_http.py" "$@"

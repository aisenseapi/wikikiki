#!/usr/bin/env bash
# Compatibility entrypoint. The Python client preserves file bytes and revisions.
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec python3 "$script_dir/wikikiki-sync.py" "$@"

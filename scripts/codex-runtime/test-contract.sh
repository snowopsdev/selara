#!/bin/sh
# Production binary smoke test; no authentication or model requests.
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
exec python3 "$repo_dir/scripts/codex-runtime/test-production.py" "${1:-$repo_dir/target/selara-codex}"

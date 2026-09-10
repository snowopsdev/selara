#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
source_dir=$(python3 -c 'import pathlib,tomllib,sys; r=pathlib.Path(sys.argv[1]); m=tomllib.loads((r/"vendor/codex-runtime/runtime.toml").read_text()); print(r/"target/codex-runtime"/("source-"+m["patches_sha256"])/"codex-rs")' "$repo_dir")
test -f "$source_dir/Cargo.toml" || { echo 'Build the pinned runtime source first' >&2; exit 2; }
export RUSTUP_TOOLCHAIN=1.95.0
export CARGO_TARGET_DIR="$repo_dir/target/codex-runtime/test-cargo"
exec cargo test --manifest-path "$source_dir/Cargo.toml" --locked -p codex-login --lib selara_ -- --nocapture

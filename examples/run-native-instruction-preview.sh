#!/bin/sh
# Build and open the custom-instruction popover preview against the
# production Swift sources. Extra arguments go to the preview, e.g.
#   sh examples/run-native-instruction-preview.sh --dark --state recall
set -eu
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
build_dir="$repo_root/target/native-instruction-preview"
mkdir -p "$build_dir"
xcrun swiftc -swift-version 5 -O -module-name SelaraInstructionPreview \
  "$repo_root"/apps/selara/native/Instruction*.swift \
  "$repo_root/examples/native-instruction-preview/main.swift" \
  -o "$build_dir/selara-instruction-preview"
exec "$build_dir/selara-instruction-preview" "$@"
